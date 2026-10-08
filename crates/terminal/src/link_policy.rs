//! What a click on a terminal hyperlink may do without asking first.
//!
//! A link's destination is chosen by whatever printed it (see the crate's
//! security boundary), and an OSC 8 link's visible text need not resemble it.
//! The scheme therefore decides what a click does, not the text the user saw:
//! web and mail links open directly, and anything else is handed to the view
//! as [`Event::ConfirmOpenUrl`](crate::Event::ConfirmOpenUrl) so the user can
//! see the destination before an OS handler for an arbitrary scheme runs.
//! `file:` links never get here: the click turns them into a
//! [`MaybeNavigationTarget::PathLike`](crate::MaybeNavigationTarget::PathLike),
//! which the view reveals rather than opens.
//!
//! The policy is one function so that OSC 8 links and URLs detected in the
//! text, and the direct and mouse-mode click paths, cannot drift apart.

use std::fmt::Write as _;

/// Schemes a click opens without confirmation.
const DIRECT_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// Whether a click may hand `url` straight to the OS opener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlOpenPolicy {
    Open,
    Confirm,
}

/// Decides how a clicked URL is opened. Anything without a well-formed
/// scheme, including a leading space or control character, needs confirmation.
pub fn url_open_policy(url: &str) -> UrlOpenPolicy {
    match url_scheme(url) {
        Some(scheme)
            if DIRECT_SCHEMES
                .iter()
                .any(|direct| scheme.eq_ignore_ascii_case(direct)) =>
        {
            UrlOpenPolicy::Open
        }
        _ => UrlOpenPolicy::Confirm,
    }
}

/// RFC 3986: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`.
fn url_scheme(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(scheme)
}

/// Link text made safe to show in a tooltip or a confirmation.
///
/// Bidi overrides and other invisible format characters can make a URI read
/// differently from what a click opens, and control characters have no
/// business in one. They are shown escaped, as `\u{202E}`, rather than
/// dropped, so a link that relies on them visibly looks wrong.
pub fn display_safe_link_text(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() || is_format_character(c) {
            let _ = write!(shown, "\\u{{{:04X}}}", u32::from(c));
        } else {
            shown.push(c);
        }
    }
    shown
}

/// Unicode general category `Cf` (format characters), which includes every
/// bidi control and the zero-width characters.
fn is_format_character(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

#[cfg(test)]
#[path = "tests/link_policy.rs"]
mod tests;
