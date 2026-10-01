//! The frames Zetta's modals are drawn in: the backdrop behind a modal, the
//! panel that holds it, and the pieces the two families of modal share.
//!
//! There are two families. A *palette* is anchored under the window chrome and
//! is a query over a list — the command palette, the multi-command prompt, the
//! pane theme picker, the overlay style picker. A *dialog* is centred and is a
//! short form — the close confirmation, the session prompt, the serial console,
//! the remote-session picker, and the settings dialog's own modals. Each used to
//! spell its frame out, which is how thirteen backdrops came to state three
//! different opacities (none of which applied — see [`crate::ui_tokens::SCRIM`])
//! and how only two of them stopped the wheel from scrolling the terminal
//! underneath.
//!
//! What a surface still decides for itself: its width, what it holds, where its
//! keyboard focus lives, and what clicking the backdrop does
//! ([`BackdropClick`]), because that last one genuinely differs.

use gpui::{
    AnyElement, App, Div, ElementId, IntoElement, MouseButton, ParentElement, Pixels, SharedString,
    Stateful, Styled, Window, div, relative,
};
use theme::ThemeColors;
use ui::prelude::*;

use crate::ui_tokens::{RADIUS_SURFACE, SCRIM};

/// Where a modal sits in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Placement {
    /// Under the window chrome, at the inset [`crate::ui_tokens::overlay_top_inset`]
    /// gives: where a palette opens, so the query sits near the tabs it acts on.
    UnderChrome(Pixels),
    /// In the middle of the window, for a dialog.
    Centered,
}

/// What dismissing a modal from its backdrop runs.
type DismissHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// What a click on the backdrop, outside the modal, does.
pub(crate) enum BackdropClick {
    /// Nothing: the click is absorbed and the modal stays. For a modal holding
    /// something a stray click must not throw away — a typed passphrase, or a
    /// live preview the user has not applied.
    Swallow,
    /// Dismisses the modal, as Esc would. For a palette, whose whole state is a
    /// query that is cheap to type again.
    Dismiss(DismissHandler),
}

impl BackdropClick {
    pub(crate) fn dismiss(on_dismiss: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self::Dismiss(Box::new(on_dismiss))
    }
}

/// The backdrop behind a modal, covering the window.
///
/// It occludes, so neither the wheel nor the pointer reaches the terminal under
/// it: a modal that let the terminal scroll behind it read as not modal at all.
pub(crate) fn modal_backdrop(
    id: impl Into<ElementId>,
    placement: Placement,
    click: BackdropClick,
) -> Stateful<Div> {
    let backdrop = div()
        .id(id)
        .absolute()
        .inset_0()
        .px_4()
        .flex()
        .justify_center()
        .bg(SCRIM)
        .occlude();
    let backdrop = match placement {
        Placement::UnderChrome(inset) => backdrop.pt(inset).items_start(),
        Placement::Centered => backdrop.py_4().items_center(),
    };
    match click {
        BackdropClick::Swallow => {
            backdrop.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        }
        BackdropClick::Dismiss(on_dismiss) => {
            backdrop.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                on_dismiss(window, cx);
            })
        }
    }
}

/// The panel a modal is drawn on: rounded, bordered, raised, and clipping what
/// it holds. A click inside it stays inside it, so it never reaches the
/// backdrop's [`BackdropClick`].
///
/// The caller sets its width and padding.
pub(crate) fn modal_panel(id: impl Into<ElementId>, colors: &ThemeColors) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(RADIUS_SURFACE)
        .border_1()
        .border_color(colors.border)
        .bg(colors.elevated_surface_background)
        .text_color(colors.text)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// A dialog's panel: [`modal_panel`] at `width`, never wider than most of the
/// window, with the padding and spacing every dialog uses.
pub(crate) fn dialog_panel(
    id: impl Into<ElementId>,
    width: Pixels,
    colors: &ThemeColors,
) -> Stateful<Div> {
    modal_panel(id, colors)
        .w(width)
        .max_w(relative(0.9))
        .max_h(relative(0.9))
        .p_4()
        .gap_3()
}

/// A dialog's title.
pub(crate) fn dialog_title(title: impl Into<SharedString>, colors: &ThemeColors) -> Label {
    Label::new(title)
        .size(LabelSize::Large)
        .color(Color::Custom(colors.text))
}

/// The row a dialog's buttons sit in, right-aligned with the primary button
/// last.
pub(crate) fn dialog_buttons() -> Div {
    h_flex().justify_end().gap_2()
}

/// A palette's header: the glyph that says what kind of palette it is, in the
/// accent colour, then the query.
pub(crate) fn palette_header(
    glyph: &'static str,
    query: impl IntoElement,
    colors: &ThemeColors,
) -> Div {
    div()
        .h_12()
        .px_3()
        .flex()
        .items_center()
        .text_color(colors.text)
        .child(div().text_color(colors.text_accent).mr_2().child(glyph))
        .child(query)
}

/// A section of a palette below its header. Every section draws the rule above
/// itself, so a palette with nothing between its header and footer draws one
/// rule rather than two.
pub(crate) fn palette_section(colors: &ThemeColors) -> Div {
    div().border_t_1().border_color(colors.border)
}

/// A palette's footer line: a count, a note, or the keys it answers to.
pub(crate) fn palette_footer(colors: &ThemeColors) -> Div {
    palette_section(colors)
        .min_h_7()
        .px_3()
        .py_1()
        .flex()
        .items_center()
        .text_xs()
        .text_color(colors.text_muted)
}

/// What a list shows when nothing matches the query.
pub(crate) fn empty_list_row(text: impl Into<SharedString>, colors: &ThemeColors) -> Div {
    div()
        .h_12()
        .px_3()
        .flex()
        .items_center()
        .text_sm()
        .text_color(colors.text_muted)
        .child(text.into())
}

/// One row of a palette's list: its text, with the characters the query
/// matched in the accent colour. The caller adds what leads or trails the text
/// and what clicking does.
///
/// Selection is the fill and is not overridden by hover — the palette and the
/// theme picker used to let the pointer's hover paint over the selected row, so
/// the keyboard's position vanished under a resting pointer.
pub(crate) fn picker_row(
    id: impl Into<ElementId>,
    text: SharedString,
    query: &str,
    selected: bool,
    colors: &ThemeColors,
) -> (Stateful<Div>, AnyElement) {
    let row = div()
        .id(id)
        .h_9()
        .w_full()
        .px_3()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .cursor_pointer()
        .text_sm()
        .text_color(colors.text)
        .when(selected, |row| row.bg(colors.element_selected))
        .when(!selected, |row| {
            row.hover(|style| style.bg(colors.element_hover))
        });
    (row, highlighted_text(text, query, colors))
}

/// `text` with the characters `query` matched in the accent colour, clipped
/// with an ellipsis rather than wrapping.
pub(crate) fn highlighted_text(
    text: SharedString,
    query: &str,
    colors: &ThemeColors,
) -> AnyElement {
    let highlight = gpui::HighlightStyle {
        color: Some(colors.text_accent),
        ..Default::default()
    };
    let ranges = crate::fuzzy_match::matched_ranges(&text, query);
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(
            gpui::StyledText::new(text)
                .with_highlights(ranges.into_iter().map(|range| (range, highlight))),
        )
        .into_any_element()
}

/// The keys a surface answers to, as one line: each key followed by what it
/// does, joined by `·`.
///
/// One separator and one order everywhere, where the surfaces used to mix
/// `·`, `•`, colons, double spaces and a sentence of prose.
pub(crate) fn key_hints(hints: &[(&str, &str)]) -> SharedString {
    hints
        .iter()
        .map(|(keys, verb)| format!("{keys} {verb}"))
        .collect::<Vec<_>>()
        .join(" · ")
        .into()
}

/// How a key hint names the platform's primary modifier: the one Zetta's own
/// shortcuts use where another platform would use Ctrl. Only the prompts that
/// name a modifier chord — the remote-session picker and the serial console —
/// need it.
#[cfg(any(feature = "zmux", feature = "serial-console"))]
pub(crate) const PRIMARY_MODIFIER: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else {
    "Ctrl"
};

/// How a key hint names Alt, which macOS labels Option. Only the
/// remote-session picker names it.
#[cfg(feature = "zmux")]
pub(crate) const ALT_MODIFIER: &str = if cfg!(target_os = "macos") {
    "Option"
} else {
    "Alt"
};

/// A line of [`key_hints`], or any other note, in the muted hint style.
pub(crate) fn hint_line(text: impl Into<SharedString>, colors: &ThemeColors) -> Div {
    div()
        .text_xs()
        .text_color(colors.text_muted)
        .child(text.into())
}

/// A modal, placed: `panel` over its backdrop. For the surfaces that need no
/// more than that, which is all of them but the ones with focus on the backdrop.
pub(crate) fn modal(backdrop: Stateful<Div>, panel: impl IntoElement) -> AnyElement {
    backdrop.child(panel).into_any_element()
}

#[cfg(test)]
#[path = "tests/overlay_frame.rs"]
mod tests;
