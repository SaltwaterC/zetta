//! Single-line text editing and the keys every field answers to.
//!
//! Zetta's overlays each grew their own `(query, cursor, select_all)` triple —
//! the command palette, the tab search, the rename prompt, the pickers, the
//! serial console's baud field — and each answered a different set of keys as a
//! result. Paste reached three of them, copy none, cut nowhere at all, and which
//! was which was invisible until someone tried.
//!
//! [`TextField`] is the storage all of them now share, and
//! [`apply_text_field_key`] is the editing half of their key handling, so a
//! surface's own handler keeps only the keys that are actually its own
//! (`escape`, `enter`, list navigation) and the editing keys are decided once.
//! [`crate::text_edit_ui`] is the rendering half.
//!
//! Not every field wants all of this. A masked secret must not be copyable — see
//! `session_auth_ui`, which keeps its own paste-only handling — and a constrained
//! entry like the overlay picker's two-character hex field has no value worth
//! putting on a clipboard. The serial console's baud field spends `left`/`right`
//! and `up`/`down` on cycling standard rates, so it holds a [`TextField`] but
//! keeps its own handler.

use gpui::{App, ClipboardItem, Keystroke};

/// A single-line field: its text, the cursor's byte offset into it, and whether
/// the whole value is selected.
///
/// The canonical storage for every one of Zetta's own text fields. Selection is
/// all-or-nothing: `Ctrl-A` selects the lot, and typing or pasting over it
/// replaces the lot, so there is no partial range to track. The cursor is always
/// a `char` boundary, which is what the boundary helpers below are for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextField {
    pub text: String,
    pub cursor: usize,
    pub select_all: bool,
}

impl TextField {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            select_all: false,
        }
    }

    /// Like [`Self::new`], but with the whole value selected, so the first thing
    /// typed replaces it. What an edit prompt opened over existing text wants.
    ///
    /// Deliberately not [`Self::select_all`], which selects nothing when the
    /// text is empty: an overlay is edited from an empty buffer routinely, and
    /// the caret marker its rendering shows depends on the selection being set.
    pub(crate) fn selected(text: impl Into<String>) -> Self {
        Self {
            select_all: true,
            ..Self::new(text)
        }
    }

    /// Replaces the selection with `text`, or inserts at the cursor when
    /// nothing is selected.
    ///
    /// Newlines are dropped rather than inserted: every field this serves is a
    /// single line, and a pasted command or path that carries a trailing newline
    /// is the common case rather than the exotic one. A cursor a surface left
    /// past the end of the text is clamped rather than panicking.
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        let text = text.replace(['\r', '\n'], "");
        let cursor = self.cursor.min(self.text.len());
        self.text.insert_str(cursor, &text);
        self.cursor = cursor + text.len();
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            let previous = previous_char_boundary(&self.text, self.cursor);
            self.text.replace_range(previous..self.cursor, "");
            self.cursor = previous;
        }
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.text.len() {
            let next = next_char_boundary(&self.text, self.cursor);
            self.text.replace_range(self.cursor..next, "");
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = if self.select_all {
            0
        } else {
            previous_char_boundary(&self.text, self.cursor)
        };
        self.select_all = false;
    }

    pub fn move_right(&mut self) {
        self.cursor = if self.select_all {
            self.text.len()
        } else {
            next_char_boundary(&self.text, self.cursor)
        };
        self.select_all = false;
    }

    pub(crate) fn move_to_start(&mut self) {
        self.cursor = 0;
        self.select_all = false;
    }

    pub(crate) fn move_to_end(&mut self) {
        self.cursor = self.text.len();
        self.select_all = false;
    }

    pub fn select_all(&mut self) {
        self.select_all = !self.text.is_empty();
    }

    /// The text a copy or a cut takes, or `None` when there is nothing to take.
    ///
    /// The whole value when nothing is selected, rather than nothing at all.
    /// With no partial range to copy, a shortcut that only worked after `Ctrl-A`
    /// would be a two-step dance for the one thing it can mean — and "does
    /// nothing" is the dead end that sent people looking for this.
    pub(crate) fn selected_text(&self) -> Option<&str> {
        (!self.text.is_empty()).then_some(self.text.as_str())
    }

    /// The cursor, clamped to the text, and the text either side of it — what
    /// every rendering of a field needs and none of them should compute itself.
    pub(crate) fn split_at_cursor(&self) -> (&str, &str) {
        self.text.split_at(self.cursor.min(self.text.len()))
    }

    /// The value with a `|` standing in for the caret, for the three surfaces
    /// that show a field as text rather than as an element: a tab or pane being
    /// renamed, a pane overlay being typed, and the serial console's baud field.
    ///
    /// A selected value is shown whole, since the selection is what is
    /// highlighted; an empty selected value still shows the marker, so an
    /// overlay opened on a pane with no text is visibly being edited.
    pub(crate) fn caret_marker_display(&self) -> String {
        if self.select_all {
            if self.text.is_empty() {
                return "|".to_owned();
            }
            return self.text.clone();
        }
        let (before, after) = self.split_at_cursor();
        format!("{before}|{after}")
    }

    fn delete_selection(&mut self) -> bool {
        if !self.select_all {
            return false;
        }
        self.text.clear();
        self.cursor = 0;
        self.select_all = false;
        true
    }
}

/// The byte offset of the `char` before `cursor`, or the start of the text.
pub(crate) fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// The byte offset just past the `char` at `cursor`, or the end of the text.
pub(crate) fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(text.len())
}

/// What a clipboard shortcut did, so the surface knows whether to re-run
/// whatever its text drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardOutcome {
    /// Not a clipboard shortcut. The caller carries on with its own handling.
    Ignored,
    /// Handled, and the text is unchanged — a copy, or a shortcut with nothing
    /// to act on.
    Unchanged,
    /// Handled, and the text changed — a cut or a paste.
    Edited,
}

fn command(keystroke: &Keystroke) -> bool {
    keystroke.modifiers.control || keystroke.modifiers.platform
}

/// Whether this keystroke asks a field to copy.
///
/// Four spellings, because four are in circulation and a text field is the wrong
/// place to have an opinion about which: `Ctrl-C` and `Cmd-C` are the platform
/// defaults, `Ctrl-Shift-C` is the habit a terminal builds — where plain
/// `Ctrl-C` interrupts — and `Ctrl-Insert` is the older convention this
/// application already honours in the terminal.
///
/// Letters are compared case-insensitively so the shifted spelling is caught
/// whatever the platform reports for it.
pub(crate) fn is_copy_chord(keystroke: &Keystroke) -> bool {
    command(keystroke) && (keystroke.key.eq_ignore_ascii_case("c") || keystroke.key == "insert")
}

/// Whether this keystroke asks a field to cut.
///
/// A deliberately narrower set than the copies, because the obvious spellings
/// are already taken by something far worse than a missing shortcut:
///
/// - `Ctrl-Shift-X` is **Close All Windows** inside a terminal pane, and every
///   overlay but the settings dialog runs in that context — so binding cut to it
///   would close the window from the tab search box.
/// - `Cmd-X` is **Close All Windows** on macOS with no context at all, so it
///   fires everywhere, the settings dialog included.
///
/// A key binding is dispatched before any key-down listener, so neither could be
/// reclaimed by handling it here: the action wins and the field never sees the
/// keystroke. `Ctrl-X` and `Shift-Delete` are bound to nothing and are what cut
/// uses. See `close_all_windows_keybinding` for the other side of this.
pub(crate) fn is_cut_chord(keystroke: &Keystroke) -> bool {
    (keystroke.modifiers.control
        && !keystroke.modifiers.shift
        && keystroke.key.eq_ignore_ascii_case("x"))
        || (keystroke.modifiers.shift && keystroke.key == "delete")
}

/// Whether this keystroke asks a field to paste. `Shift-Insert` joins
/// `Ctrl-V`/`Cmd-V`/`Ctrl-Shift-V`.
pub(crate) fn is_paste_chord(keystroke: &Keystroke) -> bool {
    (command(keystroke) && keystroke.key.eq_ignore_ascii_case("v"))
        || (keystroke.modifiers.shift && keystroke.key == "insert")
}

/// Carries out whichever clipboard shortcut `keystroke` names, if any.
///
/// Checked before a surface's own key handling, because these chords have to win
/// over the plain keys they are built from — `x` types an `x`, `Ctrl-X` does not
/// — and because `delete` and `insert` already mean something on their own.
pub(crate) fn apply_clipboard_shortcut(
    field: &mut TextField,
    keystroke: &Keystroke,
    cx: &App,
) -> ClipboardOutcome {
    if is_copy_chord(keystroke) {
        let Some(text) = field.selected_text() else {
            return ClipboardOutcome::Unchanged;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        return ClipboardOutcome::Unchanged;
    }
    if is_cut_chord(keystroke) {
        // Cut takes the same text a copy would, so the two cannot disagree about
        // what "the value" is. Nothing is lost by the wide reading: whatever it
        // removes is on the clipboard, and none of these fields is written
        // anywhere until its surface is committed.
        let Some(text) = field.selected_text() else {
            return ClipboardOutcome::Unchanged;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        field.select_all();
        field.insert("");
        return ClipboardOutcome::Edited;
    }
    if is_paste_chord(keystroke) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return ClipboardOutcome::Unchanged;
        };
        if text.is_empty() {
            return ClipboardOutcome::Unchanged;
        }
        field.insert(&text);
        return ClipboardOutcome::Edited;
    }
    ClipboardOutcome::Ignored
}

/// What a keystroke did to a field, so a surface knows whether to re-run
/// whatever its text drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextFieldEdit {
    /// Not a key this field answers to. The caller carries on with its own
    /// handling.
    Ignored,
    /// The cursor or the selection moved and the text is unchanged, so a redraw
    /// is all this needs.
    CursorMoved,
    /// The text changed. Whatever the field drives — a match list, a completion,
    /// a search — no longer describes it.
    Edited,
}

/// Applies whichever editing key `keystroke` names to `field`.
///
/// Every field-bearing surface answers the same set: `backspace`, `delete`,
/// `left`, `right`, `home`, `end`, `Ctrl`/`Cmd-A`, and a printable character.
/// A surface matches its own keys first — `escape`, `enter`, list navigation —
/// and delegates the rest here, so what is left in its handler is only what is
/// actually specific to it.
///
/// Call [`apply_clipboard_shortcut`] before this, not after: `Shift-Delete` has
/// to cut rather than forward-delete, and `Ctrl-X` has to cut rather than type
/// an `x`.
pub(crate) fn apply_text_field_key(field: &mut TextField, keystroke: &Keystroke) -> TextFieldEdit {
    match keystroke.key.as_str() {
        "backspace" => {
            field.backspace();
            TextFieldEdit::Edited
        }
        "delete" => {
            field.delete();
            TextFieldEdit::Edited
        }
        "left" => {
            field.move_left();
            TextFieldEdit::CursorMoved
        }
        "right" => {
            field.move_right();
            TextFieldEdit::CursorMoved
        }
        "home" => {
            field.move_to_start();
            TextFieldEdit::CursorMoved
        }
        "end" => {
            field.move_to_end();
            TextFieldEdit::CursorMoved
        }
        // Compared case-insensitively for the same reason the clipboard chords
        // are: the shifted spelling is what some platforms report, and a field
        // is the wrong place to have an opinion about which.
        key if command(keystroke) && key.eq_ignore_ascii_case("a") => {
            field.select_all();
            TextFieldEdit::CursorMoved
        }
        _ => {
            // A modified character is a chord aimed at the surface, not text:
            // the field only takes what the platform resolved to a character.
            let unmodified = !command(keystroke) && !keystroke.modifiers.alt;
            match keystroke.key_char.as_ref().filter(|_| unmodified) {
                Some(text) => {
                    field.insert(text);
                    TextFieldEdit::Edited
                }
                None => TextFieldEdit::Ignored,
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/text_edit.rs"]
mod tests;
