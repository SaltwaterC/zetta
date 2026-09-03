use super::*;

fn keystroke(key: &str, modifiers: gpui::Modifiers) -> gpui::Keystroke {
    gpui::Keystroke {
        modifiers,
        key: key.to_owned(),
        key_char: None,
    }
}

/// A keystroke the platform resolved to a character, which is what a field
/// takes as text.
fn typed(character: &str) -> gpui::Keystroke {
    gpui::Keystroke {
        modifiers: gpui::Modifiers::default(),
        key: character.to_owned(),
        key_char: Some(character.to_owned()),
    }
}

fn plain(key: &str) -> gpui::Keystroke {
    keystroke(key, gpui::Modifiers::default())
}

fn control() -> gpui::Modifiers {
    gpui::Modifiers {
        control: true,
        ..Default::default()
    }
}

fn platform() -> gpui::Modifiers {
    gpui::Modifiers {
        platform: true,
        ..Default::default()
    }
}

fn shift() -> gpui::Modifiers {
    gpui::Modifiers {
        shift: true,
        ..Default::default()
    }
}

fn control_shift() -> gpui::Modifiers {
    gpui::Modifiers {
        control: true,
        shift: true,
        ..Default::default()
    }
}

/// Every spelling of copy a field is likely to be asked for. Before this, none
/// of them did anything at all in any of Zetta's own text fields.
#[test]
fn every_spelling_of_copy_is_recognized() {
    for (key, modifiers) in [
        ("c", control()),
        ("c", platform()),
        ("c", control_shift()),
        // Whatever the platform reports for the shifted letter.
        ("C", control_shift()),
        ("insert", control()),
    ] {
        assert!(
            is_copy_chord(&keystroke(key, modifiers)),
            "{modifiers:?}-{key} should copy"
        );
    }
}

#[test]
fn every_spelling_of_cut_is_recognized() {
    for (key, modifiers) in [("x", control()), ("X", control()), ("delete", shift())] {
        assert!(
            is_cut_chord(&keystroke(key, modifiers)),
            "{modifiers:?}-{key} should cut"
        );
    }
}

/// The two spellings a cut would otherwise want are already **Close All
/// Windows**: `Ctrl-Shift-X` inside a terminal pane, which is the context every
/// overlay but the settings dialog runs in, and `Cmd-X` globally on macOS.
/// Bindings are dispatched before key-down listeners, so claiming either here
/// would not win the keystroke — it would only mean the window closed when
/// someone meant to cut.
#[test]
fn cut_leaves_the_close_all_windows_spellings_alone() {
    for (key, modifiers) in [
        ("x", control_shift()),
        ("X", control_shift()),
        ("x", platform()),
    ] {
        assert!(
            !is_cut_chord(&keystroke(key, modifiers)),
            "{modifiers:?}-{key} is Close All Windows and must not cut"
        );
    }
}

#[test]
fn every_spelling_of_paste_is_recognized() {
    for (key, modifiers) in [
        ("v", control()),
        ("v", platform()),
        ("v", control_shift()),
        ("V", control_shift()),
        ("insert", shift()),
    ] {
        assert!(
            is_paste_chord(&keystroke(key, modifiers)),
            "{modifiers:?}-{key} should paste"
        );
    }
}

/// The three must not overlap, and none may claim a plain key: `x` types an `x`,
/// and bare `delete` and `insert` keep meaning what they always did.
#[test]
fn the_chords_do_not_overlap_or_claim_the_plain_keys() {
    type Chord = fn(&gpui::Keystroke) -> bool;
    let chords: [Chord; 3] = [is_copy_chord, is_cut_chord, is_paste_chord];
    for (key, modifiers) in [
        ("c", control()),
        ("x", control()),
        ("v", control()),
        ("insert", control()),
        ("insert", shift()),
        ("delete", shift()),
    ] {
        let matched = chords
            .iter()
            .filter(|matches| matches(&keystroke(key, modifiers)))
            .count();
        assert_eq!(matched, 1, "{modifiers:?}-{key} should match exactly one");
    }
    for key in ["c", "x", "v", "insert", "delete", "a"] {
        let plain = keystroke(key, gpui::Modifiers::default());
        assert!(
            !is_copy_chord(&plain) && !is_cut_chord(&plain) && !is_paste_chord(&plain),
            "a plain {key} must not be a clipboard shortcut"
        );
    }
}

/// `Ctrl-A` selects, and `Ctrl-Shift-A` must not be mistaken for a cut or a
/// copy just because the letters are adjacent on the same modifier.
#[test]
fn select_all_is_not_a_clipboard_chord() {
    for modifiers in [control(), platform(), control_shift()] {
        let key = keystroke("a", modifiers);
        assert!(!is_copy_chord(&key) && !is_cut_chord(&key) && !is_paste_chord(&key));
    }
}

#[test]
fn copying_and_cutting_take_the_whole_value_when_nothing_is_selected() {
    let field = TextField::new("age1example");
    assert_eq!(field.selected_text(), Some("age1example"));

    let mut selected = TextField::new("~/keys/zetta.txt");
    selected.select_all();
    assert_eq!(selected.selected_text(), Some("~/keys/zetta.txt"));

    assert_eq!(TextField::default().selected_text(), None);
}

#[test]
fn pasting_replaces_a_selection_and_inserts_at_the_cursor_otherwise() {
    let mut field = TextField::new("abcdef");
    field.cursor = 3;
    field.insert("XY");
    assert_eq!(field.text, "abcXYdef");
    assert_eq!(field.cursor, 5);

    let mut selected = TextField::new("abcdef");
    selected.select_all();
    selected.insert("XY");
    assert_eq!(selected.text, "XY");
    assert_eq!(selected.cursor, 2);
    assert!(!selected.select_all);
}

/// Every field this serves is a single line, and a copied path or command
/// routinely carries a trailing newline.
#[test]
fn pasting_strips_newlines_rather_than_inserting_them() {
    let mut field = TextField::default();
    field.insert("one\r\ntwo\n");
    assert_eq!(field.text, "onetwo");
}

/// A cut leaves the field empty and its cursor at the start, which is what the
/// surfaces' own rendering assumes of a cleared field.
#[test]
fn cutting_clears_the_field_and_resets_its_cursor() {
    let mut field = TextField::new("age1example");
    field.select_all();
    field.insert("");
    assert!(field.text.is_empty());
    assert_eq!(field.cursor, 0);
    assert!(!field.select_all);
}

/// Inserting is clamped rather than panicking if a surface left its cursor past
/// the end — several of them track it by hand.
#[test]
fn inserting_past_the_end_of_the_text_is_clamped() {
    let mut field = TextField::new("abc");
    field.cursor = 99;
    field.insert("d");
    assert_eq!(field.text, "abcd");
    assert_eq!(field.cursor, 4);
}

/// Multi-byte text is why the char-boundary helpers exist: a cursor that steps
/// by bytes would land inside a character and panic on the next edit.
#[test]
fn editing_steps_over_whole_characters() {
    let mut field = TextField::new("café");
    assert_eq!(field.cursor, 5);

    assert_eq!(
        apply_text_field_key(&mut field, &plain("left")),
        TextFieldEdit::CursorMoved
    );
    assert_eq!(field.cursor, 3);
    assert_eq!(
        apply_text_field_key(&mut field, &plain("backspace")),
        TextFieldEdit::Edited
    );
    assert_eq!(field.text, "caé");
    assert_eq!(field.cursor, 2);

    apply_text_field_key(&mut field, &plain("end"));
    apply_text_field_key(&mut field, &plain("left"));
    apply_text_field_key(&mut field, &plain("delete"));
    assert_eq!(field.text, "ca");
}

#[test]
fn typing_over_a_selection_replaces_the_whole_value() {
    let mut field = TextField::new("115200");
    apply_text_field_key(&mut field, &keystroke("a", control()));
    assert!(field.select_all);

    assert_eq!(
        apply_text_field_key(&mut field, &typed("9")),
        TextFieldEdit::Edited
    );
    assert_eq!(field.text, "9");
    assert_eq!(field.cursor, 1);
    assert!(!field.select_all);
}

/// A selection is dropped by the keys that move rather than edit, and the cursor
/// lands at the end the arrow points to.
#[test]
fn a_selection_collapses_to_the_end_the_cursor_moves_toward() {
    let mut selected_left = TextField::new("abc");
    selected_left.select_all();
    apply_text_field_key(&mut selected_left, &plain("left"));
    assert_eq!(selected_left.cursor, 0);
    assert!(!selected_left.select_all);

    let mut selected_right = TextField::new("abc");
    selected_right.cursor = 0;
    selected_right.select_all();
    apply_text_field_key(&mut selected_right, &plain("right"));
    assert_eq!(selected_right.cursor, 3);
    assert!(!selected_right.select_all);
}

/// `Ctrl-A` on an empty field selects nothing, so a surface that renders the
/// selection cannot show one over no text.
#[test]
fn selecting_an_empty_field_selects_nothing() {
    let mut field = TextField::default();
    apply_text_field_key(&mut field, &keystroke("a", platform()));
    assert!(!field.select_all);
}

/// A cursor a surface left past the end is clamped rather than panicking, and
/// deleting at either end of the text is a no-op rather than an error.
#[test]
fn editing_at_the_ends_of_the_text_is_clamped() {
    let mut past_the_end = TextField::new("abc");
    past_the_end.cursor = 99;
    apply_text_field_key(&mut past_the_end, &plain("delete"));
    assert_eq!(past_the_end.text, "abc");

    let mut at_the_start = TextField::new("abc");
    at_the_start.move_to_start();
    apply_text_field_key(&mut at_the_start, &plain("backspace"));
    assert_eq!(at_the_start.text, "abc");
    assert_eq!(at_the_start.cursor, 0);
}

/// The keys a surface keeps for itself have to come back as `Ignored`, or an
/// overlay's `escape` and list navigation would be swallowed by its field.
#[test]
fn the_surfaces_own_keys_are_left_alone() {
    for key in ["escape", "enter", "up", "down", "tab", "f3", "insert"] {
        let mut field = TextField::new("abc");
        assert_eq!(
            apply_text_field_key(&mut field, &plain(key)),
            TextFieldEdit::Ignored,
            "{key} belongs to the surface"
        );
        assert_eq!(field, TextField::new("abc"));
    }
    // A modified character is a chord aimed at the surface rather than text.
    let mut field = TextField::new("abc");
    let mut chord = typed("b");
    chord.modifiers = control();
    assert_eq!(
        apply_text_field_key(&mut field, &chord),
        TextFieldEdit::Ignored
    );
    assert_eq!(field.text, "abc");
}

/// What the surfaces branch on: an edit re-runs the search, the completion or
/// the match list, and a cursor move only redraws.
#[test]
fn only_the_keys_that_change_the_text_report_an_edit() {
    for (key, expected) in [
        ("backspace", TextFieldEdit::Edited),
        ("delete", TextFieldEdit::Edited),
        ("left", TextFieldEdit::CursorMoved),
        ("right", TextFieldEdit::CursorMoved),
        ("home", TextFieldEdit::CursorMoved),
        ("end", TextFieldEdit::CursorMoved),
    ] {
        let mut field = TextField::new("abc");
        field.cursor = 1;
        assert_eq!(
            apply_text_field_key(&mut field, &plain(key)),
            expected,
            "{key}"
        );
    }
}

/// The `|` marker is how a renamed tab, a pane label, a pane overlay and the
/// baud field show a caret they cannot paint.
#[test]
fn the_caret_marker_shows_the_cursor_in_plain_text() {
    let mut field = TextField::new("Database");
    field.cursor = 4;
    assert_eq!(field.caret_marker_display(), "Data|base");

    field.select_all();
    assert_eq!(field.caret_marker_display(), "Database");

    // An overlay is opened selected on a pane that has no text yet, and has to
    // look like it is being edited all the same.
    assert_eq!(TextField::selected("").caret_marker_display(), "|");
}
