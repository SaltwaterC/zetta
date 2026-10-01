//! How a Zetta surface says that something failed, needs attention, or worked.
//!
//! Before this, every surface picked its own: the settings dialog told an error
//! from an update only by `text` against `text_muted`, the profile modal showed
//! both in `text`, template validation in `text` on a box, and the prompts each
//! used the status error colour at a different size and place. A reader could
//! not tell a failed save from a progress note at a glance. [`status_message`]
//! is the one line of feedback every form and prompt uses, and [`Tone`] is the
//! one vocabulary for how serious it is.

use gpui::{Div, Hsla, SharedString, div};
use theme::{StatusColors, ThemeColors};
use ui::prelude::*;

/// How serious a message is, which decides its colour and its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Something the user asked for did not happen.
    Error,
    /// It happened, but not the way the user might expect.
    Warning,
    /// Progress, or a note about what just happened.
    Info,
    /// Confirmation that something the user asked for did happen.
    Success,
}

impl Tone {
    /// The text colour. Info is the muted text colour rather than the status
    /// palette's blue: it is the tone of an ordinary note, and most themes'
    /// info colour reads as a link.
    pub(crate) fn color(self, colors: &ThemeColors, status: &StatusColors) -> Hsla {
        match self {
            Tone::Error => status.error,
            Tone::Warning => status.warning,
            Tone::Info => colors.text_muted,
            Tone::Success => status.success,
        }
    }

    pub(crate) fn icon(self) -> IconName {
        match self {
            Tone::Error => IconName::XCircle,
            Tone::Warning => IconName::Warning,
            Tone::Info => IconName::Info,
            Tone::Success => IconName::Check,
        }
    }
}

/// One line of feedback: the tone's icon, then the text, both in the tone's
/// colour. Wraps rather than clipping, since what went wrong is usually the
/// longest part of the sentence.
pub(crate) fn status_message(
    tone: Tone,
    text: impl Into<SharedString>,
    colors: &ThemeColors,
    status: &StatusColors,
) -> Div {
    let color = tone.color(colors, status);
    h_flex()
        .w_full()
        .items_start()
        .gap_1p5()
        .text_xs()
        .text_color(color)
        .child(
            div().flex_none().pt(px(1.)).child(
                Icon::new(tone.icon())
                    .size(IconSize::XSmall)
                    .color(Color::Custom(color)),
            ),
        )
        .child(div().min_w_0().flex_1().child(text.into()))
}

/// [`status_message`] for an error, where the caller already holds the
/// theme's error colour rather than the whole status palette — as the prompts
/// do, which resolve it once per frame.
pub(crate) fn error_message(text: impl Into<SharedString>, error_color: Hsla) -> Div {
    h_flex()
        .w_full()
        .items_start()
        .gap_1p5()
        .text_sm()
        .text_color(error_color)
        .child(
            div().flex_none().pt(px(2.)).child(
                Icon::new(Tone::Error.icon())
                    .size(IconSize::XSmall)
                    .color(Color::Custom(error_color)),
            ),
        )
        .child(div().min_w_0().flex_1().child(text.into()))
}

#[cfg(test)]
#[path = "tests/ui_messages.rs"]
mod tests;
