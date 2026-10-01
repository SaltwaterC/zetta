//! The numbers Zetta's own surfaces are drawn with: how dark a modal's backdrop
//! is, how round a corner, how wide a settings control, how far a disabled
//! control fades.
//!
//! Colours are not here. They come from the theme, and a surface names the
//! `ThemeColors` field that plays each role; what the theme cannot say is the
//! geometry, and until this module every surface restated it — three backdrop
//! opacities for the same kind of modal, five corner radii, two widths for the
//! same settings control, and three ways of fading a disabled button. A value
//! that recurs belongs here; a one-off (a colour swatch, the performance graph)
//! stays with its surface and says why.

use gpui::{Hsla, Pixels, Window, px, transparent_black};

/// The backdrop behind a modal: transparent, which is what Zetta has always
/// drawn.
///
/// Every modal used to spell its backdrop `transparent_black().opacity(..)`
/// with 0.24, 0.3 or 0.55. `opacity` scales the colour's alpha, and
/// `transparent_black` has an alpha of zero, so all three were zero: the
/// differing values were three spellings of "invisible", and no modal has ever
/// dimmed the window. That is kept here as the one deliberate value rather than
/// every modal silently starting to dim. To dim them all, make this
/// `gpui::black().opacity(..)`.
pub(crate) const SCRIM: Hsla = transparent_black();

/// Corners of a floating surface: a modal, a picker, a popover's frame.
pub(crate) const RADIUS_SURFACE: Pixels = px(8.);
/// Corners of a control or a card inside a surface: fields, buttons, the cards
/// a settings page lists.
pub(crate) const RADIUS_CONTROL: Pixels = px(4.);
/// Corners of a row inside a popover list, which sits closer to its frame's
/// edge than a control does.
pub(crate) const RADIUS_ROW: Pixels = px(3.);

/// How far a control that cannot be used right now fades.
pub(crate) const DISABLED_OPACITY: f32 = 0.5;

/// The width of the control column in a settings row, whether the row has a
/// description under its label or not. One width, so a field is the same size
/// on every page.
pub(crate) const CONTROL_COLUMN_WIDTH: Pixels = px(330.);
/// A settings row with a label and a description.
pub(crate) const ROW_MIN_HEIGHT: Pixels = px(54.);
/// A settings row with a label only, on the denser pages.
pub(crate) const DENSE_ROW_MIN_HEIGHT: Pixels = px(42.);

/// The command palette, the multi-command prompt and the pickers that share
/// their frame.
pub(crate) const PALETTE_WIDTH: Pixels = px(680.);
/// How tall a palette's list may grow before it scrolls.
pub(crate) const PALETTE_LIST_MAX_HEIGHT: Pixels = px(360.);

/// Widths of a centred dialog: a confirmation, a prompt with a couple of
/// fields, and a prompt with a list.
pub(crate) const DIALOG_WIDTH_SMALL: Pixels = px(440.);
pub(crate) const DIALOG_WIDTH_MEDIUM: Pixels = px(560.);
pub(crate) const DIALOG_WIDTH_LARGE: Pixels = px(680.);

/// The gap between the window chrome and an overlay anchored under it.
const OVERLAY_GAP: Pixels = px(8.);

/// How far from the top of the window an overlay anchored under the chrome
/// starts: the chrome's own height, which compact mode changes, plus a gap.
/// These used to be a fixed 72 or 74 pixels, which was right for the full
/// chrome only and left a band of empty space in compact mode.
pub(crate) fn overlay_top_inset(compact_mode: bool, window: &Window) -> Pixels {
    crate::title_bar_render::title_bar_chrome_height(
        compact_mode,
        crate::window_frame::platform_title_bar_height(window),
        window.rem_size(),
    ) + OVERLAY_GAP
}

#[cfg(test)]
#[path = "tests/ui_tokens.rs"]
mod tests;
