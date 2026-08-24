//! Single-line text editing and the clipboard shortcuts every field answers to.
//!
//! Zetta's overlays each grew their own `(query, cursor, select_all)` triple
//! before [`crate::settings_editor::TextField`] existed — the command palette,
//! the tab search, the rename prompt, the pickers, the serial console's baud
//! field — and each answered a different set of keys as a result. Paste reached
//! three of them, copy none, cut nowhere at all, and which was which was
//! invisible until someone tried.
//!
//! [`TextEdit`] is the one shape all of them can present, borrowed from wherever
//! the surface happens to keep it, so a shortcut is decided once here instead of
//! being re-implemented per overlay. It deliberately does not try to unify the
//! surfaces' storage: the point is that they no longer have to agree on that to
//! agree on what `Ctrl-C` does.
//!
//! Not every field wants this. A masked secret must not be copyable — see
//! `session_auth_ui`, which keeps its own paste-only handling — and a constrained
//! entry like the overlay picker's two-character hex field has no value worth
//! putting on a clipboard.

use gpui::{App, ClipboardItem, Keystroke};

/// A single-line field's editable state, borrowed from its owner.
///
/// Selection is all-or-nothing, which is what every one of these surfaces
/// implements: `Ctrl-A` selects the lot, and typing or pasting over it replaces
/// the lot. There is no partial range to track.
pub(crate) struct TextEdit<'a> {
    text: &'a mut String,
    cursor: &'a mut usize,
    select_all: &'a mut bool,
}

impl<'a> TextEdit<'a> {
    pub(crate) fn new(
        text: &'a mut String,
        cursor: &'a mut usize,
        select_all: &'a mut bool,
    ) -> Self {
        Self {
            text,
            cursor,
            select_all,
        }
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

    /// Replaces the selection with `text`, or inserts at the cursor when nothing
    /// is selected.
    ///
    /// Newlines are dropped rather than inserted: every field this serves is a
    /// single line, and a pasted command or path that carries a trailing newline
    /// is the common case rather than the exotic one.
    pub(crate) fn insert(&mut self, text: &str) {
        self.delete_selection();
        let text = text.replace(['\r', '\n'], "");
        let cursor = (*self.cursor).min(self.text.len());
        self.text.insert_str(cursor, &text);
        *self.cursor = cursor + text.len();
    }

    pub(crate) fn select_all(&mut self) {
        *self.select_all = !self.text.is_empty();
    }

    fn delete_selection(&mut self) -> bool {
        if !*self.select_all {
            return false;
        }
        self.text.clear();
        *self.cursor = 0;
        *self.select_all = false;
        true
    }
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
    mut edit: TextEdit<'_>,
    keystroke: &Keystroke,
    cx: &App,
) -> ClipboardOutcome {
    if is_copy_chord(keystroke) {
        let Some(text) = edit.selected_text() else {
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
        let Some(text) = edit.selected_text() else {
            return ClipboardOutcome::Unchanged;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        edit.select_all();
        edit.insert("");
        return ClipboardOutcome::Edited;
    }
    if is_paste_chord(keystroke) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return ClipboardOutcome::Unchanged;
        };
        if text.is_empty() {
            return ClipboardOutcome::Unchanged;
        }
        edit.insert(&text);
        return ClipboardOutcome::Edited;
    }
    ClipboardOutcome::Ignored
}

#[cfg(test)]
#[path = "tests/text_edit.rs"]
mod tests;
