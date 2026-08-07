use std::sync::{Arc, OnceLock};

use strum::IntoEnumIterator as _;

use crate::TextField;
use gpui::ScrollHandle;
use ui::IconName;

/// An icon paired with its precomputed, lowercased search label, so
/// filtering never has to re-run `format!("{icon:?}")` per keystroke.
pub(crate) type IconEntry = (IconName, String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabIconPickerTarget {
    Tab(usize),
    Default,
}

pub(crate) struct TabIconPicker {
    pub(crate) target: TabIconPickerTarget,
    pub(crate) query: TextField,
    pub(crate) selected: usize,
    pub(crate) scroll: ScrollHandle,
    options_cache: Vec<Option<IconName>>,
    options_query_cache: String,
}

impl TabIconPicker {
    pub(crate) fn new(
        target: TabIconPickerTarget,
        current_icon: Option<IconName>,
        entries: &[IconEntry],
    ) -> Self {
        let options = filter_icon_entries(entries, "");
        let selected = options
            .iter()
            .position(|icon| *icon == current_icon)
            .unwrap_or_default();
        Self {
            target,
            query: TextField::default(),
            selected,
            scroll: ScrollHandle::new(),
            options_cache: options,
            options_query_cache: String::new(),
        }
    }

    /// The icon options matching the picker's current search query, shared
    /// by keyboard handling and rendering so both see identical results and
    /// neither redoes the filter when the query hasn't changed since the
    /// last call.
    pub(crate) fn options(&mut self, entries: &[IconEntry]) -> &[Option<IconName>] {
        if self.options_query_cache != self.query.text {
            self.options_cache = filter_icon_entries(entries, &self.query.text);
            self.options_query_cache = self.query.text.clone();
        }
        &self.options_cache
    }
}

pub(crate) fn tab_icon_label(icon: IconName) -> String {
    format!("{icon:?}")
}

pub(crate) fn build_icon_entries(icons: &[IconName]) -> Vec<IconEntry> {
    icons
        .iter()
        .map(|icon| (*icon, tab_icon_label(*icon).to_lowercase()))
        .collect()
}

/// Fallback icon entries for use before the background-populated icon
/// cache on `Zetta` is ready. Computed at most once per process.
pub(crate) fn fallback_icon_entries() -> Arc<[IconEntry]> {
    static FALLBACK: OnceLock<Arc<[IconEntry]>> = OnceLock::new();
    FALLBACK
        .get_or_init(|| build_icon_entries(&IconName::iter().collect::<Vec<_>>()).into())
        .clone()
}

fn filter_icon_entries(entries: &[IconEntry], query: &str) -> Vec<Option<IconName>> {
    let query = query.to_lowercase();
    let mut options = Vec::new();
    if query.is_empty() || "none".contains(&query) || "no icon".contains(&query) {
        options.push(None);
    }
    options.extend(
        entries
            .iter()
            .filter(|(_, label)| label.contains(&query))
            .map(|(icon, _)| Some(*icon)),
    );
    options
}

/// Convenience wrapper over the fallback (uncached) icon entries; only
/// used by tests, which exercise filtering without going through a
/// `TabIconPicker`/`Zetta` instance.
#[cfg(test)]
pub(crate) fn matching_tab_icons(query: &str) -> Vec<IconName> {
    filter_icon_entries(&fallback_icon_entries(), query)
        .into_iter()
        .flatten()
        .collect()
}

/// Convenience wrapper over the fallback (uncached) icon entries; only
/// used by tests, which exercise filtering without going through a
/// `TabIconPicker`/`Zetta` instance.
#[cfg(test)]
pub(crate) fn matching_tab_icon_options(query: &str) -> Vec<Option<IconName>> {
    filter_icon_entries(&fallback_icon_entries(), query)
}

pub(crate) fn tab_icon_completion_names() -> impl Iterator<Item = &'static str> {
    std::iter::once("none").chain(IconName::iter().map(|icon| {
        let name: &'static str = icon.into();
        name
    }))
}

pub(crate) fn parse_tab_icon_name(name: &str) -> Option<IconName> {
    IconName::iter().find(|icon| {
        let cli_name: &'static str = (*icon).into();
        cli_name.eq_ignore_ascii_case(name) || tab_icon_label(*icon).eq_ignore_ascii_case(name)
    })
}

#[cfg(test)]
#[path = "tests/tab_icon_picker.rs"]
mod tests;
