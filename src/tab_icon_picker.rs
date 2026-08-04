use strum::IntoEnumIterator as _;

use crate::TextField;
use gpui::ScrollHandle;
use ui::IconName;

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
}

impl TabIconPicker {
    pub(crate) fn new(target: TabIconPickerTarget, current_icon: Option<IconName>) -> Self {
        let selected = matching_tab_icon_options("")
            .iter()
            .position(|icon| *icon == current_icon)
            .unwrap_or_default();
        Self {
            target,
            query: TextField::default(),
            selected,
            scroll: ScrollHandle::new(),
        }
    }
}

pub(crate) fn tab_icon_label(icon: IconName) -> String {
    format!("{icon:?}")
}

pub(crate) fn matching_tab_icons(query: &str) -> Vec<IconName> {
    let query = query.to_lowercase();
    IconName::iter()
        .filter(|icon| tab_icon_label(*icon).to_lowercase().contains(&query))
        .collect()
}

pub(crate) fn matching_tab_icon_options(query: &str) -> Vec<Option<IconName>> {
    let query = query.to_lowercase();
    let mut options = Vec::new();
    if query.is_empty() || "none".contains(&query) || "no icon".contains(&query) {
        options.push(None);
    }
    options.extend(matching_tab_icons(&query).into_iter().map(Some));
    options
}

#[cfg(test)]
#[path = "tests/tab_icon_picker.rs"]
mod tests;
