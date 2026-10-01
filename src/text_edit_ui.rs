//! The rendering half of a single-line field.
//!
//! Zetta draws text fields in two arrangements, and until this module each
//! surface spelled its own out. The overlays (the command palette, the tab
//! search, the theme picker, the multi-command prompt) render a *query run*: the
//! text either side of a caret, inline in a header row that supplies its own
//! frame. The prompts and the settings forms render a *boxed field*: the same run
//! inside a bordered, `editor_background` box that shows focus.
//!
//! Both are here, as separate pieces rather than one widget with a flag for
//! every difference: each site still decides its own width, placeholder, click
//! target, and — for the boxed ones — what an unfocused field shows. What is
//! shared is the part that has to look identical everywhere, which is the run
//! itself and the box's frame.
//!
//! [`crate::text_edit`] owns the state these render.

use gpui::{Div, SharedString, Stateful, div, px};
use theme::ThemeColors;
use ui::prelude::*;

use crate::text_edit::TextField;

/// The caret. One pixel wide and the accent colour, so it reads as a cursor
/// rather than as a character.
///
/// Its height is the line height of the text around it, which it gets by
/// holding a zero-width space: a fixed height was right for the settings
/// fields' text and too short for a pane overlay typed at a larger size, which
/// is why those surfaces used to draw a `|` into their text instead.
pub(crate) fn caret(colors: &ThemeColors) -> Div {
    div()
        .flex_none()
        .w(px(1.))
        .overflow_hidden()
        .bg(colors.text_accent)
        .child("\u{200B}")
}

/// A field's text with its caret in it, laid out to be clipped rather than to
/// wrap.
///
/// A selected value carries the selection background and shows no caret: the
/// selection is what marks the position, and a caret inside it would read as a
/// second cursor. `placeholder` is rendered only when it is `Some`, so a field
/// with nothing to suggest emits no element for it.
pub(crate) fn field_text_run(
    before: SharedString,
    after: SharedString,
    selected: bool,
    placeholder: Option<SharedString>,
    colors: &ThemeColors,
) -> Div {
    h_flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .when(selected, |input| {
            input.bg(colors.element_selection_background)
        })
        .when(selected && before.is_empty() && after.is_empty(), |input| {
            // Nothing is selected to show where the field is, so it keeps its
            // caret: an overlay opens selected on a pane with no text yet, and
            // has to look like it is being edited all the same.
            input.child(caret(colors))
        })
        .child(div().whitespace_nowrap().child(before))
        .when(!selected, |input| input.child(caret(colors)))
        .child(div().whitespace_nowrap().child(after))
        .when_some(placeholder, |input, placeholder| {
            input.child(div().text_color(colors.text_placeholder).child(placeholder))
        })
}

/// [`field_text_run`] for a field held whole, with `placeholder` shown while it
/// is empty.
pub(crate) fn field_query_run(
    field: &TextField,
    placeholder: Option<&'static str>,
    colors: &ThemeColors,
) -> Div {
    let (before, after) = field.split_at_cursor();
    field_text_run(
        before.to_owned().into(),
        after.to_owned().into(),
        field.select_all,
        placeholder
            .filter(|_| field.text.is_empty())
            .map(SharedString::from),
        colors,
    )
}

/// The frame a boxed field draws: one row high, bordered, and filled with the
/// editor background, with the border reporting focus.
///
/// The caller adds its own width — these appear both full-width in a form and as
/// flex items in a row — along with `cursor_text` where the box is clickable and
/// whatever it wants inside.
pub(crate) fn field_box(
    id: impl Into<ElementId>,
    focused: bool,
    colors: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .h_9()
        .px_2()
        .flex()
        .items_center()
        .overflow_hidden()
        .rounded(crate::ui_tokens::RADIUS_CONTROL)
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .bg(colors.editor_background)
}

/// How a boxed field shows its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldMask {
    Plain,
    /// One bullet per character, for passphrases and session secrets.
    Secret,
}

/// A boxed field with its value in it: [`field_box`]'s frame, holding the
/// [`field_text_run`] and its caret while focused, and otherwise the value
/// clipped with an ellipsis — or `placeholder`, in the placeholder colour,
/// while the value is empty.
///
/// This is the one boxed field: the settings forms, the remote-session picker
/// and the session prompt each used to spell out this same focused/unfocused
/// split, and had drifted in where the selection highlight went and whether an
/// empty field said anything. The caller still adds its width, its click
/// target and `cursor_text`.
pub(crate) fn boxed_text_field(
    id: impl Into<ElementId>,
    field: &TextField,
    focused: bool,
    placeholder: Option<SharedString>,
    mask: FieldMask,
    colors: &ThemeColors,
) -> Stateful<Div> {
    let masked = |text: &str| -> SharedString {
        match mask {
            FieldMask::Plain => text.to_owned().into(),
            FieldMask::Secret => "•".repeat(text.chars().count()).into(),
        }
    };
    let frame = field_box(id, focused, colors);
    if focused {
        let (before, after) = field.split_at_cursor();
        return frame.child(field_text_run(
            masked(before),
            masked(after),
            field.select_all,
            placeholder.filter(|_| field.text.is_empty()),
            colors,
        ));
    }
    let (text, color) = match placeholder {
        Some(placeholder) if field.text.is_empty() => (placeholder, colors.text_placeholder),
        _ => (masked(&field.text), colors.text),
    };
    frame.child(
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(color)
            .child(text),
    )
}

/// A value shown where a field would be, for a form that cannot be edited — a
/// built-in template, say. Same height and padding as [`field_box`], so a form
/// does not reflow between its editable and read-only states, but no border to
/// suggest it can be typed into.
pub(crate) fn read_only_field(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    colors: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .h_9()
        .px_2()
        .flex()
        .items_center()
        .overflow_hidden()
        .rounded(crate::ui_tokens::RADIUS_CONTROL)
        .bg(colors.element_background)
        .text_color(colors.text_muted)
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text.into()),
        )
}

#[cfg(test)]
#[path = "tests/text_edit_ui.rs"]
mod tests;
